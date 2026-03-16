using System.ComponentModel;
using System.Reflection;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Veta.ViewModels;

public partial class ViewModelObservables : ObservableObject
{
    private string _displayLanguage;

    public ViewModelObservables()
    {
        _displayLanguage = "en";
    }

    public string DisplayLanguage
    {
        get => _displayLanguage;
        set => SetProperty(ref _displayLanguage, value);
    }

    protected override void OnPropertyChanged(PropertyChangedEventArgs e)
    {
        base.OnPropertyChanged(e);
        if (e.PropertyName == nameof(DisplayLanguage))
        {
            foreach (PropertyInfo prop in GetType().GetProperties(BindingFlags.Public | BindingFlags.Instance))
            {
                if (prop.PropertyType == typeof(string) && prop.Name != nameof(DisplayLanguage))
                    OnPropertyChanged(prop.Name);
            }
        }
    }
}
